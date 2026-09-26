//! The dynamic recompiler: translates blocks of guest instructions into
//! host machine code, as DOSBox's dynamic core does, and runs them in
//! place of the interpreter. See docs/dynrec.md.
//!
//! It is exact: every instruction sees the same instruction count (the
//! emulated time), interrupts arrive between the same instructions and
//! faults leave the same state as with the interpreter. Blocks only hold
//! instructions that can't change anything the interpreter checks between
//! instructions (see `block`), are entered only when they fit before the
//! next timer event, and bring the instruction count up to date before
//! each instruction that could read it. Instructions it doesn't translate
//! run through their interpreter handlers, called from the block.

#[cfg(dynrec)]
use crate::cpu::CpuModel;
use crate::cpu::{Cpu, Fault};
use crate::exec::At;

#[cfg(dynrec)]
mod block;
#[cfg(dynrec)]
mod codemem;
#[cfg(dynrec)]
mod flags;
#[cfg(dynrec)]
mod helpers;
#[cfg(dynrec)]
mod translate;
#[cfg(dynrec)]
mod uop;
#[cfg(all(dynrec, target_arch = "x86_64"))]
mod x64;
#[cfg(all(dynrec, target_arch = "aarch64"))]
mod a64;

/// Whether this build has a code generator for its host.
pub const AVAILABLE: bool = cfg!(dynrec);

/// Most instructions in a block.
#[cfg(dynrec)]
const MAX_BLOCK: usize = 64;
/// Bytes of host code for translated blocks: room for games like the
/// Doom engine's, whose unrolled drawing loops are entered at every row
/// and column they can start at, a chain of blocks from each (Heretic's
/// translated code comes to about 60 MB). Only the pages code was written
/// to take memory.
#[cfg(dynrec)]
const CODE_SIZE: usize = 128 << 20;

/// Counts for the statistics.
#[derive(Clone, Copy, Debug, Default)]
pub struct DynStats {
    /// Blocks translated, and their guest instructions, of which these
    /// were translated into host code (the rest call their handlers).
    pub blocks: u64,
    pub instructions: u64,
    pub native: u64,
    /// Blocks translated now, their host code's bytes, and the links
    /// between them.
    pub live_blocks: u64,
    pub code_bytes: u64,
    pub links: u64,
    /// Times all translated code was thrown away.
    pub flushes: u64,
    /// Blocks run from the execution loop, and the instructions the
    /// translated code ran.
    pub runs: u64,
    pub executed: u64,
    /// Blocks that didn't fit before the timer deadline, whose bytes had
    /// changed, and that wrote over their own instructions.
    pub deadline: u64,
    pub stale: u64,
    pub smc: u64,
    /// Blocks that stopped at an instruction whose watched bytes had
    /// changed.
    pub watched: u64,
}

/// What `DynState::run` did.
#[cfg_attr(not(dynrec), allow(dead_code))]
pub(crate) enum Run {
    /// Nothing: the interpreter runs the instruction.
    Interpret,
    /// Translated code ran; EIP is where it stopped. `page` is the page the
    /// last instruction was in (see `Page`).
    Ran { page: Page },
    /// An instruction faulted. It was undone, but for being counted as
    /// executed: EIP is on it and ESP as before. The execution loop
    /// delivers the fault and counts the instruction.
    Fault { fault: Fault, phys_ip: usize, page: Page },
    /// An instruction's handler panicked.
    Panic(Box<dyn std::any::Any + Send>),
}

/// The linear and physical address of the page translated code ran its
/// last instruction in. Blocks linked across pages move on without the
/// execution loop, whose code window the interpreter would have moved to
/// that page (see `exec::CodeWindow`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Page {
    pub lin: u32,
    pub phys: usize,
}

/// The recompiler's state: its translated code and what it knows about it.
#[derive(Default)]
pub struct DynState {
    #[cfg(dynrec)]
    engine: Option<Box<engine::Engine>>,
    /// The code memory couldn't be set up: the interpreter runs everything.
    #[cfg(dynrec)]
    unavailable: bool,
    /// Bytes of host code to reserve, if not `CODE_SIZE`.
    code_size: Option<usize>,
    stats: DynStats,
}

impl DynState {
    /// Forget all translated code.
    pub fn flush(&mut self) {
        #[cfg(dynrec)]
        if let Some(engine) = &mut self.engine {
            engine.flush(&mut self.stats);
        }
    }

    pub fn stats(&self) -> DynStats {
        #[cfg(dynrec)]
        if let Some(engine) = &self.engine {
            return DynStats { links: engine.links(), ..self.stats };
        }
        self.stats
    }

    /// The counts but for the links, which take counting: for every
    /// frame.
    pub fn counts(&self) -> DynStats {
        self.stats
    }

    /// Reserve `bytes` for host code from now on, instead of 128 MB: the
    /// tests fill a small one. Forgets all translated code.
    pub fn set_code_size(&mut self, bytes: usize) {
        #[cfg(dynrec)]
        {
            self.engine = None;
        }
        self.code_size = Some(bytes);
    }

    /// Run translated code from the instruction at `at`, which is in the
    /// interpreter's code window, translating it first if need be. With
    /// `single`, blocks hold one instruction (`Cpu::step`).
    #[cfg(dynrec)]
    pub(crate) fn run(&mut self, cpu: &mut Cpu, at: &At, single: bool) -> Run {
        if self.engine.is_none() {
            if self.unavailable {
                return Run::Interpret;
            }
            match engine::Engine::new(cpu.model, self.code_size.unwrap_or(CODE_SIZE)) {
                Ok(engine) => self.engine = Some(Box::new(engine)),
                Err(e) => {
                    cpu.bus.log_string(&format!("[DYNREC] No memory for translated code ({}), interpreting", e));
                    self.unavailable = true;
                    return Run::Interpret;
                }
            }
        }
        let engine = self.engine.as_mut().unwrap();
        // The translated code counts its instructions as it returns, as
        // the interpreter counts each.
        let before = cpu.executed;
        let run = engine.run(cpu, at, single, &mut self.stats);
        self.stats.executed += cpu.executed.wrapping_sub(before);
        run
    }

    #[cfg(not(dynrec))]
    pub(crate) fn run(&mut self, _cpu: &mut Cpu, _at: &At, _single: bool) -> Run {
        Run::Interpret
    }
}

#[cfg(dynrec)]
mod engine {
    use std::collections::HashMap;
    use std::ptr::NonNull;

    use super::block::{BlockData, Guard, LINKS, RETURN_LINK, RETURN_MISS, WATCH_AFTER};
    use crate::cpu::{CR0_PG, Seg};
    use super::codemem::CodeMemory;
    use super::helpers::*;
    #[cfg(target_arch = "aarch64")]
    use super::a64 as backend;
    #[cfg(target_arch = "x86_64")]
    use super::x64 as backend;
    use super::*;

    /// What a block is translated for: where its bytes are, the EIP they
    /// were decoded at, and how.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct Key {
        phys: u32,
        eip: u32,
        /// Bit 0: 32-bit code; bit 1: one instruction (`Cpu::step`); bit 2:
        /// a 32-bit stack.
        mode: u8,
    }

    impl std::hash::Hash for Key {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            state.write_u64((self.phys as u64) << 32 | self.eip as u64 ^ (self.mode as u64) << 60);
        }
    }

    /// A hasher for keys, which hash as one word: a multiply is enough.
    #[derive(Default)]
    struct KeyHasher(u64);

    impl std::hash::Hasher for KeyHasher {
        fn write(&mut self, bytes: &[u8]) {
            for &b in bytes {
                self.write_u64(b as u64);
            }
        }

        fn write_u64(&mut self, v: u64) {
            self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x517C_C1B7_2722_0A95);
        }

        fn finish(&self) -> u64 {
            self.0
        }
    }

    type KeyMap = HashMap<Key, u32, std::hash::BuildHasherDefault<KeyHasher>>;

    impl Key {
        fn index(&self) -> usize {
            (self.phys ^ self.eip.rotate_left(13) ^ (self.mode as u32) << 20).wrapping_mul(0x9E37_79B1) as usize
                >> (32 - FRONT_BITS)
        }
    }

    /// A translated block: its code, its guest block (owned: freed when the
    /// block is), and the links of other blocks to it.
    struct Block {
        key: Key,
        code: *const u8,
        /// Bytes of the code.
        len: usize,
        /// Which translation this is: the index and the code's place are
        /// reused when blocks go.
        serial: u64,
        data: NonNull<BlockData>,
        /// Blocks (index, link) whose link leads here, and the block each
        /// of this one's links leads to: each link is in the backlinks of
        /// that block, once.
        backlinks: Vec<(u32, u8)>,
        targets: [Option<u32>; LINKS],
        /// The return link a return to none of their places takes over
        /// next, once all are made.
        next_return: u8,
    }

    impl Drop for Block {
        fn drop(&mut self) {
            // SAFETY: the data was leaked from a box when the block was
            // made, and nothing runs the block's code any more.
            drop(unsafe { Box::from_raw(self.data.as_ptr()) });
        }
    }

    /// log2 of the slots of the direct-mapped table in front of the map,
    /// and of the table of places where no block starts.
    const FRONT_BITS: u32 = 14;
    const NONE_BITS: u32 = 12;

    /// The code generations of the chunks an instruction at `phys` can be
    /// in (it is at most 15 bytes long).
    fn instr_gens(page_gen: &[u32], phys: u32) -> u32 {
        let (first, last) = (phys as usize >> crate::bus::GEN_SHIFT, (phys as usize + 14) >> crate::bus::GEN_SHIFT);
        page_gen[first].wrapping_add(page_gen[last])
    }

    /// A link to another page (or a return's) that the execution loop
    /// makes when it runs the block at `eip` next: the block `from` (the
    /// translation `serial`, in case the index has been reused) left
    /// through link `slot` for it. Links are made while nothing was thrown
    /// away (`flushes`), between blocks of one `mode`.
    #[derive(Clone, Copy)]
    struct Pending {
        from: u32,
        serial: u64,
        slot: u8,
        eip: u32,
        mode: u8,
        flushes: u64,
    }

    pub struct Engine {
        mem: CodeMemory,
        pending: Option<Pending>,
        enter: backend::Enter,
        ctx: Box<JitCtx>,
        blocks: Vec<Option<Block>>,
        free: Vec<u32>,
        map: KeyMap,
        /// Places where no block starts (the interpreter runs the
        /// instruction: an emulator service trap, HLT), with the code
        /// generations of the instruction's chunks then, direct-mapped.
        none: Box<[Option<(Key, u32)>]>,
        /// Block index + 1 for a key's slot, 0 for none.
        front: Box<[u32]>,
        /// Per physical page, how often each byte was poked: changed where
        /// a block went stale or wrote over itself (see
        /// `block::WATCH_AFTER`).
        pokes: HashMap<u32, Box<[u8]>>,
        /// The CPU model the blocks' handlers were chosen for.
        model: CpuModel,
    }

    impl Engine {
        pub fn new(model: CpuModel, code_size: usize) -> std::io::Result<Self> {
            let mut mem = CodeMemory::new(code_size)?;
            let tramp = backend::trampoline();
            let base = mem.add(&tramp.bytes).expect("room for the trampoline") as usize;
            mem.keep();
            // SAFETY: `enter` is the trampoline's entry, whose signature
            // `Enter` is.
            let enter = unsafe { std::mem::transmute::<usize, backend::Enter>(base + tramp.enter) };
            Ok(Engine {
                mem,
                pending: None,
                enter,
                ctx: Box::new(JitCtx::new(base + tramp.exit)),
                blocks: Vec::new(),
                free: Vec::new(),
                map: KeyMap::default(),
                none: vec![None; 1 << NONE_BITS].into_boxed_slice(),
                front: vec![0; 1 << FRONT_BITS].into_boxed_slice(),
                pokes: HashMap::new(),
                model,
            })
        }

        pub fn flush(&mut self, stats: &mut DynStats) {
            self.blocks.clear();
            self.free.clear();
            self.map.clear();
            self.none.fill(None);
            self.front.fill(0);
            self.pokes.clear();
            self.mem.clear();
            stats.flushes += 1;
            stats.live_blocks = 0;
            stats.code_bytes = 0;
        }

        /// Links between blocks now.
        pub fn links(&self) -> u64 {
            self.blocks.iter().flatten().map(|b| b.backlinks.len() as u64).sum()
        }

        /// The block for `key`, if there is one.
        #[inline(always)]
        fn lookup(&mut self, key: Key) -> Option<u32> {
            let slot = key.index();
            let front = self.front[slot];
            if front != 0 && self.blocks[front as usize - 1].as_ref().is_some_and(|b| b.key == key) {
                return Some(front - 1);
            }
            let index = *self.map.get(&key)?;
            self.front[slot] = index + 1;
            Some(index)
        }

        /// The block for `key` at `at`, translating it if need be; None if
        /// no block starts there.
        fn find(&mut self, cpu: &Cpu, at: &At, key: Key, stats: &mut DynStats) -> Option<u32> {
            if let Some(index) = self.lookup(key) {
                return Some(index);
            }
            let none = key.index() >> (FRONT_BITS - NONE_BITS);
            let gens = instr_gens(&cpu.bus.page_gen, key.phys);
            if self.none[none] == Some((key, gens)) {
                return None;
            }
            let found = self.translate(cpu, at, key, stats);
            if found.is_none() {
                self.none[none] = Some((key, gens));
            }
            found
        }

        /// Translate the block at `at`. None if no block starts there.
        fn translate(&mut self, cpu: &Cpu, at: &At, key: Key, stats: &mut DynStats) -> Option<u32> {
            let single = key.mode & 2 != 0;
            let pokes = self.pokes.get(&(key.phys >> 12)).map(|p| &p[..]);
            let data = BlockData::build(at, cpu.bus.ram(), &cpu.bus.page_gen, if single { 1 } else { MAX_BLOCK }, pokes)?;
            let stack32 = key.mode & 4 != 0;
            let items: Vec<_> = (0..data.count())
                .map(|ix| {
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32);
                    super::translate::translate(&data.instrs[ix], next, stack32)
                })
                .collect();
            let native = items.iter().filter(|i| i.is_some()).count() as u64;
            let mut data = NonNull::from(Box::leak(Box::new(data)));
            // SAFETY: just made, and owned by the block from here on.
            let code = backend::block(unsafe { data.as_ref() }, &items, !single);
            let base = match self.mem.add(&code.bytes) {
                Some(base) => base,
                None => {
                    // Full: start over (nothing translated is running). A
                    // block too big for all of it isn't translated.
                    self.flush(stats);
                    match self.mem.add(&code.bytes) {
                        Some(base) => base,
                        None => {
                            drop(unsafe { Box::from_raw(data.as_ptr()) });
                            return None;
                        }
                    }
                }
            };
            let index = self.free.pop().unwrap_or(self.blocks.len() as u32);
            {
                // SAFETY: owned by the block, and not in use.
                let data = unsafe { data.as_mut() };
                for (k, stub) in code.stubs.iter().enumerate() {
                    if let Some(stub) = stub {
                        data.stubs[k] = base as usize + stub;
                        data.links[k] = data.stubs[k];
                    }
                }
                data.id = index;
                data.lag = code.lag;
                stats.blocks += 1;
                stats.instructions += data.count() as u64;
                stats.native += native;
                stats.live_blocks += 1;
                stats.code_bytes = self.mem.used() as u64;
            }
            let block = Block {
                key,
                code: base,
                len: code.bytes.len(),
                serial: stats.blocks,
                data,
                backlinks: Vec::new(),
                targets: [None; LINKS],
                next_return: 0,
            };
            if index as usize == self.blocks.len() {
                self.blocks.push(Some(block));
            } else {
                self.blocks[index as usize] = Some(block);
            }
            self.map.insert(key, index);
            self.front[key.index()] = index + 1;
            Some(index)
        }

        /// Link block `from` to block `to`, which the execution loop is about
        /// to run at `at` after `from` left through the link for it, with
        /// the guard of how its fetch went (see `Guard`).
        fn link_guarded(&mut self, p: Pending, to: u32, cpu: &Cpu, at: &At) {
            let page = at.lin_ip >> 12;
            let paging = cpu.cr0 & CR0_PG != 0;
            let phys = if paging {
                match cpu.tlb.lookup(page, cpu.cpl == 3) {
                    Some(phys) => phys,
                    None => return,
                }
            } else {
                0
            };
            let guard = Guard {
                eip: at.eip,
                cs_base: cpu.seg_cache(Seg::CS).base,
                a20: cpu.bus.a20_mask(),
                paging: paging as u32,
                page,
                phys,
            };
            let Some(source) = self.blocks[p.from as usize].as_mut().filter(|b| b.serial == p.serial) else { return };
            // SAFETY: owned by the block, and not in use.
            unsafe { source.data.as_mut() }.guards[p.slot as usize] = guard;
            self.link(p.from, p.slot as usize, to);
        }

        /// Point link `slot` of block `from` at block `to`. A link made
        /// again for another block (a return's, for every other place it
        /// returns to) leaves the backlinks of the one it led to: blocks
        /// that live long would otherwise gather millions of them.
        fn link(&mut self, from: u32, slot: usize, to: u32) {
            let to_code = self.blocks[to as usize].as_ref().unwrap().code;
            let source = self.blocks[from as usize].as_mut().unwrap();
            // SAFETY: owned by the block, and not in use.
            unsafe { source.data.as_mut() }.links[slot] = to_code as usize;
            let old = source.targets[slot].replace(to);
            if old == Some(to) {
                return;
            }
            if let Some(old) = old {
                self.drop_backlink(old, from, slot);
            }
            self.blocks[to as usize].as_mut().unwrap().backlinks.push((from, slot as u8));
        }

        /// Take link `slot` of block `from` out of the backlinks of block
        /// `to`.
        fn drop_backlink(&mut self, to: u32, from: u32, slot: usize) {
            if let Some(block) = self.blocks[to as usize].as_mut()
                && let Some(i) = block.backlinks.iter().position(|&l| l == (from, slot as u8))
            {
                block.backlinks.swap_remove(i);
            }
        }

        /// Note the bytes of a block that were poked since it was
        /// translated, before it is dropped.
        fn note_pokes(&mut self, ram: &[u8], data: &BlockData) {
            let Some(poked) = data.poked(ram) else { return };
            let counts = self.pokes.entry(data.phys >> 12).or_insert_with(|| vec![0; 0x1000].into_boxed_slice());
            for i in poked {
                let n = &mut counts[(data.phys as usize & 0xFFF) + i];
                *n = n.saturating_add(1).min(WATCH_AFTER);
            }
        }

        /// Drop a block whose bytes changed, and the links to it. Its code's
        /// place goes to blocks translated later: code that rewrites
        /// itself all the time (a RET poked into an unrolled loop and put
        /// back, for every span of a floor) would otherwise fill the
        /// memory, and translating everything again after it is thrown
        /// away takes long enough to hold up a video frame.
        fn retire(&mut self, index: u32, stats: &mut DynStats) {
            let Some(block) = self.blocks[index as usize].take() else { return };
            self.mem.remove(block.code, block.len);
            stats.code_bytes = self.mem.used() as u64;
            for (slot, to) in block.targets.iter().enumerate() {
                if let Some(to) = *to {
                    self.drop_backlink(to, index, slot);
                }
            }
            for &(from, k) in &block.backlinks {
                if let Some(source) = self.blocks[from as usize].as_mut() {
                    // SAFETY: owned by the block, and not in use.
                    let data = unsafe { source.data.as_mut() };
                    if data.links[k as usize] == block.code as usize {
                        data.links[k as usize] = data.stubs[k as usize];
                        source.targets[k as usize] = None;
                    }
                }
            }
            self.map.remove(&block.key);
            let slot = block.key.index();
            if self.front[slot] == index + 1 {
                self.front[slot] = 0;
            }
            self.free.push(index);
            stats.live_blocks -= 1;
        }

        pub fn run(&mut self, cpu: &mut Cpu, at: &At, single: bool, stats: &mut DynStats) -> Run {
            if cpu.model != self.model {
                // The handlers were chosen for the other model.
                self.flush(stats);
                self.model = cpu.model;
            }
            let mode = at.code32 as u8 | (single as u8) << 1 | (cpu.stack32() as u8) << 2;
            let key = Key { phys: at.phys_ip as u32, eip: at.eip, mode };
            let pending = self.pending.take();
            let Some(mut index) = self.find(cpu, at, key, stats) else { return Run::Interpret };
            if let Some(p) = pending
                && !single
                && (p.eip, p.mode, p.flushes) == (at.eip, mode, stats.flushes)
            {
                self.link_guarded(p, index, cpu, at);
            }
            // The CS base: blocks linked to each other run under one.
            let cs_base = at.lin_ip.wrapping_sub(at.eip);
            // A block that stops before its first instruction (the timer
            // deadline, the CS limit, changed bytes) leaves the instruction
            // at EIP to the interpreter if nothing ran before it, else to
            // the execution loop, which checks the deadline first.
            let start = cpu.bus.clock.icount;
            let mut retried = false;
            loop {
                let block = self.blocks[index as usize].as_ref().unwrap();
                self.ctx.ram = cpu.bus.ram().as_ptr();
                self.ctx.ram_len = cpu.bus.ram().len() as u64;
                self.ctx.tlb = cpu.tlb.entries_ptr() as *const u8;
                self.ctx.page_gen = cpu.bus.page_gen.as_ptr();
                stats.runs += 1;
                // SAFETY: the code was generated for this trampoline, and
                // gets the CPU and context it expects.
                let ret = unsafe { (self.enter)(cpu, &mut *self.ctx, block.code) };
                if let Some(payload) = self.ctx.panic.take() {
                    // Rust code the block called panicked (it went on with
                    // made-up values): nothing it did counts.
                    return Run::Panic(payload);
                }
                let (kind, ix) = (ret as u32 & 0xFF, (ret as u32 >> 8 & 0xFF) as usize);
                // SAFETY: the block the code returned from (the one entered,
                // or one linked from it) is still alive: nothing retires
                // blocks while code runs.
                let data = unsafe { &*self.ctx.exit_data };
                if matches!(kind, EXIT_FAULT | EXIT_GP0 | EXIT_DE | EXIT_SMC | EXIT_WATCHED) {
                    // Instruction ix stopped the block: it counts as executed
                    // (the interpreter counts it before running it) but not in
                    // the instruction count, which this adds once it has dealt
                    // with it. One whose watched bytes changed didn't run.
                    cpu.bus.clock.icount += data.lag[ix] as u64;
                    cpu.executed += ix as u64 + (kind != EXIT_WATCHED) as u64;
                    if ret as u32 & EXIT_FLAGS != 0 {
                        cpu.set_flag_bits(crate::cpu::alu::ARITH, self.ctx.flags);
                    }
                }
                let exited = data.id;
                let none_ran = cpu.bus.clock.icount == start;
                let page = Page { lin: cs_base.wrapping_add(data.eips[0]) & !0xFFF, phys: data.phys as usize & !0xFFF };
                return match kind {
                    EXIT_NEXT => Run::Ran { page },
                    EXIT_DEADLINE | EXIT_LIMIT => {
                        if kind == EXIT_DEADLINE {
                            stats.deadline += 1;
                        }
                        // A block linked to from another page stopped before
                        // its first instruction: fetching it would have moved
                        // the interpreter's window to its page without side
                        // effects, as the link's guard found the page in the
                        // TLB (or paging off) just now.
                        if none_ran { Run::Interpret } else { Run::Ran { page } }
                    }
                    EXIT_STALE => {
                        stats.stale += 1;
                        self.note_pokes(cpu.bus.ram(), data);
                        self.retire(exited, stats);
                        if !none_ran {
                            return Run::Ran { page };
                        }
                        if retried {
                            return Run::Interpret;
                        }
                        retried = true;
                        match self.translate(cpu, at, key, stats) {
                            Some(i) => {
                                index = i;
                                continue;
                            }
                            None => Run::Interpret,
                        }
                    }
                    EXIT_UNLINKED if ix >= RETURN_LINK || !data.in_page(cpu.eip()) => {
                        // A block left for another page, or returned: the
                        // execution loop finds the block there, as its fetch
                        // may go through the page tables, and links it. A
                        // return to a new place takes a link not made yet,
                        // or else the one after the one it took last.
                        let block = self.blocks[exited as usize].as_mut().unwrap();
                        let slot = if ix == RETURN_MISS {
                            match (RETURN_LINK..LINKS).find(|&k| block.targets[k].is_none()) {
                                Some(k) => k,
                                None => {
                                    block.next_return = (block.next_return + 1) % (LINKS - RETURN_LINK) as u8;
                                    RETURN_LINK + block.next_return as usize
                                }
                            }
                        } else {
                            ix
                        };
                        self.pending = Some(Pending {
                            from: exited,
                            serial: block.serial,
                            slot: slot as u8,
                            eip: cpu.eip(),
                            mode,
                            flushes: stats.flushes,
                        });
                        Run::Ran { page }
                    }
                    EXIT_UNLINKED => {
                        // A block left for a known EIP in its page: link it to
                        // the block there and go on in that one. Nothing a
                        // block can change stops the next from being entered
                        // as the execution loop would (see `block`).
                        let target = cpu.eip();
                        let t_at = At { eip: target, phys_ip: data.phys_in_page(target) as usize, ..*at };
                        let t_key = Key { phys: t_at.phys_ip as u32, eip: target, mode };
                        let flushes = stats.flushes;
                        let Some(t) = self.find(cpu, &t_at, t_key, stats) else { return Run::Ran { page } };
                        // (Translating it may have made room by throwing all
                        // blocks away, the one to link from with them.)
                        if stats.flushes == flushes {
                            self.link(exited, ix, t);
                        }
                        index = t;
                        continue;
                    }
                    EXIT_FAULT | EXIT_GP0 | EXIT_DE => {
                        cpu.set_eip(data.eips[ix]);
                        let fault = match kind {
                            EXIT_GP0 => Fault::gp(0),
                            EXIT_DE => Fault::DE,
                            _ => self.ctx.fault,
                        };
                        Run::Fault { fault, phys_ip: data.phys_of(ix), page }
                    }
                    EXIT_SMC => {
                        stats.smc += 1;
                        // The instruction is done; the rest of the block
                        // changed under it.
                        cpu.bus.clock.icount += 1;
                        self.note_pokes(cpu.bus.ram(), data);
                        self.retire(exited, stats);
                        Run::Ran { page }
                    }
                    EXIT_WATCHED => {
                        // The interpreter runs whatever is there now; the
                        // block stays for when the bytes are back.
                        stats.watched += 1;
                        cpu.set_eip(data.eips[ix]);
                        if none_ran { Run::Interpret } else { Run::Ran { page } }
                    }
                    _ => unreachable!("exit code {:X}", ret),
                };
            }
        }
    }
}
