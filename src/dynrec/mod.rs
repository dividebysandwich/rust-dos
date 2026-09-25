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
mod helpers;
#[cfg(all(dynrec, target_arch = "x86_64"))]
mod x64;

/// Whether this build has a code generator for its host.
pub const AVAILABLE: bool = cfg!(dynrec);

/// Most instructions in a block.
#[cfg(dynrec)]
const MAX_BLOCK: usize = 64;
/// Bytes of host code for translated blocks.
#[cfg(dynrec)]
const CODE_SIZE: usize = 32 << 20;

/// Counts for the statistics.
#[derive(Clone, Copy, Debug, Default)]
pub struct DynStats {
    /// Blocks translated, and their guest instructions.
    pub blocks: u64,
    pub instructions: u64,
    /// Blocks translated now, and their host code's bytes.
    pub live_blocks: u64,
    pub code_bytes: u64,
    /// Times all translated code was thrown away.
    pub flushes: u64,
    /// Blocks run from the execution loop.
    pub runs: u64,
    /// Blocks that didn't fit before the timer deadline, whose bytes had
    /// changed, and that wrote over their own instructions.
    pub deadline: u64,
    pub stale: u64,
    pub smc: u64,
}

/// What `DynState::run` did.
#[cfg_attr(not(dynrec), allow(dead_code))]
pub(crate) enum Run {
    /// Nothing: the interpreter runs the instruction.
    Interpret,
    /// Translated code ran; EIP is where it stopped.
    Ran,
    /// An instruction faulted. It was undone, but for being counted as
    /// executed: EIP is on it and ESP as before. The execution loop
    /// delivers the fault and counts the instruction.
    Fault { fault: Fault, phys_ip: usize },
    /// An instruction's handler panicked.
    Panic(Box<dyn std::any::Any + Send>),
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
        self.stats
    }

    /// Reserve `bytes` for host code from now on, instead of 32 MB: the
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
        engine.run(cpu, at, single, &mut self.stats)
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

    use super::block::BlockData;
    use super::codemem::CodeMemory;
    use super::helpers::*;
    use super::x64 as backend;
    use super::*;

    /// What a block is translated for: where its bytes are, the EIP they
    /// were decoded at, and how.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    struct Key {
        phys: u32,
        eip: u32,
        /// Bit 0: 32-bit code; bit 1: one instruction (`Cpu::step`).
        mode: u8,
    }

    impl Key {
        fn index(&self) -> usize {
            (self.phys ^ self.eip.rotate_left(13) ^ (self.mode as u32) << 20).wrapping_mul(0x9E37_79B1) as usize
                >> (32 - FRONT_BITS)
        }
    }

    /// A translated block: its code and its guest block (owned: freed
    /// when the block is).
    struct Block {
        key: Key,
        code: *const u8,
        data: NonNull<BlockData>,
    }

    impl Drop for Block {
        fn drop(&mut self) {
            // SAFETY: the data was leaked from a box when the block was
            // made, and nothing runs the block's code any more.
            drop(unsafe { Box::from_raw(self.data.as_ptr()) });
        }
    }

    /// log2 of the slots of the direct-mapped table in front of the map.
    const FRONT_BITS: u32 = 14;

    pub struct Engine {
        mem: CodeMemory,
        enter: backend::Enter,
        ctx: Box<JitCtx>,
        blocks: Vec<Option<Block>>,
        free: Vec<u32>,
        map: HashMap<Key, u32>,
        /// Block index + 1 for a key's slot, 0 for none.
        front: Box<[u32]>,
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
                enter,
                ctx: Box::new(JitCtx::new(base + tramp.exit)),
                blocks: Vec::new(),
                free: Vec::new(),
                map: HashMap::new(),
                front: vec![0; 1 << FRONT_BITS].into_boxed_slice(),
                model,
            })
        }

        pub fn flush(&mut self, stats: &mut DynStats) {
            self.blocks.clear();
            self.free.clear();
            self.map.clear();
            self.front.fill(0);
            self.mem.clear();
            stats.flushes += 1;
            stats.live_blocks = 0;
            stats.code_bytes = 0;
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

        /// Translate the block at `at`. None if no block starts there.
        fn translate(&mut self, cpu: &Cpu, at: &At, key: Key, stats: &mut DynStats) -> Option<u32> {
            let max = if key.mode & 2 != 0 { 1 } else { MAX_BLOCK };
            let data = BlockData::build(at, cpu.bus.ram(), &cpu.bus.page_gen, max)?;
            let data = NonNull::from(Box::leak(Box::new(data)));
            // SAFETY: just made, and owned by the block from here on.
            let bytes = backend::block(unsafe { data.as_ref() });
            let code = match self.mem.add(&bytes) {
                Some(code) => code,
                None => {
                    // Full: start over (nothing translated is running). A
                    // block too big for all of it isn't translated.
                    self.flush(stats);
                    match self.mem.add(&bytes) {
                        Some(code) => code,
                        None => {
                            drop(unsafe { Box::from_raw(data.as_ptr()) });
                            return None;
                        }
                    }
                }
            };
            stats.blocks += 1;
            stats.instructions += unsafe { data.as_ref() }.count() as u64;
            stats.live_blocks += 1;
            stats.code_bytes = self.mem.used() as u64;
            let block = Block { key, code, data };
            let index = match self.free.pop() {
                Some(index) => {
                    self.blocks[index as usize] = Some(block);
                    index
                }
                None => {
                    self.blocks.push(Some(block));
                    self.blocks.len() as u32 - 1
                }
            };
            self.map.insert(key, index);
            self.front[key.index()] = index + 1;
            Some(index)
        }

        /// Drop a block whose bytes changed.
        fn retire(&mut self, index: u32, stats: &mut DynStats) {
            if let Some(block) = self.blocks[index as usize].take() {
                self.map.remove(&block.key);
                let slot = block.key.index();
                if self.front[slot] == index + 1 {
                    self.front[slot] = 0;
                }
                self.free.push(index);
                stats.live_blocks -= 1;
            }
        }

        pub fn run(&mut self, cpu: &mut Cpu, at: &At, single: bool, stats: &mut DynStats) -> Run {
            if cpu.model != self.model {
                // The handlers were chosen for the other model.
                self.flush(stats);
                self.model = cpu.model;
            }
            let key = Key { phys: at.phys_ip as u32, eip: at.eip, mode: at.code32 as u8 | (single as u8) << 1 };
            let mut index = match self.lookup(key) {
                Some(index) => index,
                None => match self.translate(cpu, at, key, stats) {
                    Some(index) => index,
                    None => return Run::Interpret,
                },
            };
            let mut retried = false;
            loop {
                let block = self.blocks[index as usize].as_ref().unwrap();
                // SAFETY: the block owns its data.
                let data = unsafe { block.data.as_ref() };
                if data.limit_need > at.cs_limit as u64 {
                    // The interpreter would fetch part of it past the code
                    // window.
                    return Run::Interpret;
                }
                self.ctx.ram = cpu.bus.ram().as_ptr();
                self.ctx.page_gen = cpu.bus.page_gen.as_ptr();
                stats.runs += 1;
                // SAFETY: the code was generated for this trampoline, and
                // gets the CPU and context it expects.
                let ret = unsafe { (self.enter)(cpu, &mut *self.ctx, block.code) };
                let (kind, ix) = (ret as u32 & 0xFF, (ret as u32 >> 8) as usize);
                // SAFETY: the block the code returned from is still alive:
                // nothing retires blocks while code runs.
                let data = unsafe { &*self.ctx.exit_data };
                return match kind {
                    EXIT_NEXT => Run::Ran,
                    EXIT_DEADLINE => {
                        stats.deadline += 1;
                        Run::Interpret
                    }
                    EXIT_STALE => {
                        stats.stale += 1;
                        self.retire(index, stats);
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
                    EXIT_FAULT => {
                        cpu.set_eip(data.eips[ix]);
                        Run::Fault { fault: self.ctx.fault, phys_ip: data.phys_of(ix) }
                    }
                    EXIT_SMC => {
                        stats.smc += 1;
                        // The instruction is done; the rest of the block
                        // changed under it.
                        cpu.bus.clock.icount += 1;
                        self.retire(index, stats);
                        Run::Ran
                    }
                    EXIT_PANIC => Run::Panic(self.ctx.panic.take().expect("a panic")),
                    _ => unreachable!("exit code {:X}", ret),
                };
            }
        }
    }
}
