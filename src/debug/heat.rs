//! The memory heatmap (`/ws/heat`): which 64-byte blocks of RAM the machine
//! writes, glowing for a moment after each write, and which hold code.
//!
//! The writes come from the blocks' generations (`Bus::page_gen`), which
//! the interpreter and DMA bump on every write anyway; while the heatmap is
//! open the recompiler's stores bump them too (`Bus::set_write_tracking`).
//! A frame's message is binary: the block size and the number of blocks
//! (two little-endian u32s), a heat byte for each block (255 just written,
//! fading to 0), and a bit for each block that holds code (lowest bit
//! first).

use rust_dos::bus::{Bus, GEN_SHIFT};

/// How much a block's heat fades each frame: from 255 to nothing in about
/// 40 frames.
const DECAY: u8 = 6;

pub struct Heat {
    /// The generations when last looked at.
    last: Vec<u32>,
    heat: Vec<u8>,
}

impl Heat {
    /// Start watching the machine's writes.
    pub fn start(bus: &mut Bus) -> Self {
        bus.set_write_tracking(true);
        Self { last: bus.page_gen.clone(), heat: vec![0; bus.page_gen.len()] }
    }

    /// Stop watching, the recompiler's stores fast again.
    pub fn stop(bus: &mut Bus) {
        bus.set_write_tracking(false);
    }

    /// Look at the writes since the last frame.
    pub fn sample(&mut self, bus: &Bus) {
        if self.last.len() != bus.page_gen.len() {
            self.last = bus.page_gen.clone();
            self.heat = vec![0; bus.page_gen.len()];
            return;
        }
        for ((last, heat), &now) in self.last.iter_mut().zip(self.heat.iter_mut()).zip(&bus.page_gen) {
            if *last != now {
                *last = now;
                *heat = u8::MAX;
            } else {
                *heat = heat.saturating_sub(DECAY);
            }
        }
    }

    /// The frame's message.
    pub fn message(&self, bus: &Bus) -> Vec<u8> {
        let blocks = self.heat.len();
        let mut out = Vec::with_capacity(8 + blocks + blocks.div_ceil(8));
        out.extend_from_slice(&(1u32 << GEN_SHIFT).to_le_bytes());
        out.extend_from_slice(&(blocks as u32).to_le_bytes());
        out.extend_from_slice(&self.heat);
        out.extend(bus.code_blocks.chunks(8).map(|eight| {
            eight.iter().enumerate().fold(0u8, |bits, (i, &b)| bits | (((b & rust_dos::bus::CODE_BLOCK != 0) as u8) << i))
        }));
        out
    }

    /// The blocks written in the last second or so, as address ranges.
    pub fn hot_ranges(&self) -> Vec<(usize, usize)> {
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for (i, _) in self.heat.iter().enumerate().filter(|&(_, &h)| h > 0) {
            let (start, end) = (i << GEN_SHIFT, (i + 1) << GEN_SHIFT);
            match ranges.last_mut() {
                Some(last) if last.1 == start => last.1 = end,
                _ => ranges.push((start, end)),
            }
        }
        ranges
    }
}
