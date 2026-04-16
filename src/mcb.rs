//! DOS Memory Control Block (MCB) chain.
//!
//! DOS tracks conventional memory through a singly-linked chain of 16-byte
//! headers called Memory Control Blocks. Each MCB header describes the block
//! that immediately follows it:
//!
//! ```text
//! offset  size  field
//!  0x00   1     'M' (0x4D) = more blocks follow, 'Z' (0x5A) = last block
//!  0x01   2     owner PSP segment, 0 = free, 8 = DOS reserved
//!  0x03   2     size of the block in paragraphs (not counting the header)
//!  0x05   3     reserved
//!  0x08   8     owner program/file name (DOS 4+, space-padded)
//! ```
//!
//! The block's usable memory starts at the paragraph immediately after the
//! header, so a block whose MCB lives at segment `S` exposes `S + 1` as its
//! first data paragraph and spans `size` paragraphs. The next MCB is at
//! `S + 1 + size`, forming an implicit linked list.
//!
//! This module provides the helpers needed to initialize and walk the chain
//! plus the three INT 21h memory services: AH=48h (alloc), 49h (free), and
//! 4Ah (resize). Free blocks are coalesced with adjacent free blocks after
//! a free.

use crate::bus::Bus;

/// Signature byte for "more blocks follow".
pub const MCB_M: u8 = 0x4D;
/// Signature byte for "this is the last block".
pub const MCB_Z: u8 = 0x5A;

/// Segment of the MCB header of the first block. Sits just below the usual
/// program load segment (0x1000) so the chain can cover the full conventional
/// memory area [0x1000, 0xA000).
pub const FIRST_MCB_SEG: u16 = 0x0FFF;

/// Paragraph past the end of conventional memory. A0000h is the VGA VRAM
/// window, so we cannot allocate at or above it.
pub const END_OF_CONVENTIONAL: u16 = 0xA000;

/// Sentinel PSP value for an unallocated block.
pub const FREE_OWNER: u16 = 0x0000;

/// DOS error code: memory control blocks destroyed.
pub const ERR_MCB_DESTROYED: u8 = 0x07;
/// DOS error code: invalid memory block address.
pub const ERR_INVALID_BLOCK: u8 = 0x09;
/// DOS error code: insufficient memory.
pub const ERR_INSUFFICIENT: u8 = 0x08;

#[derive(Clone, Copy, Debug)]
pub struct Mcb {
    pub signature: u8,
    pub owner: u16,
    pub size: u16,
}

impl Mcb {
    pub fn is_last(&self) -> bool {
        self.signature == MCB_Z
    }

    pub fn is_free(&self) -> bool {
        self.owner == FREE_OWNER
    }

    pub fn is_valid(&self) -> bool {
        self.signature == MCB_M || self.signature == MCB_Z
    }
}

fn header_addr(seg: u16) -> usize {
    (seg as usize) * 16
}

pub fn read_mcb(bus: &Bus, seg: u16) -> Mcb {
    let base = header_addr(seg);
    Mcb {
        signature: bus.read_8(base),
        owner: bus.read_16(base + 1),
        size: bus.read_16(base + 3),
    }
}

pub fn write_mcb(bus: &mut Bus, seg: u16, mcb: &Mcb) {
    let base = header_addr(seg);
    bus.write_8(base, mcb.signature);
    bus.write_16(base + 1, mcb.owner);
    bus.write_16(base + 3, mcb.size);
    // Zero the reserved bytes and owner-name fields so the chain looks clean
    // in memory-dump utilities.
    for i in 5..16 {
        bus.write_8(base + i, 0);
    }
}

/// Walk the MCB chain from FIRST_MCB_SEG to the 'Z' sentinel. Returns the
/// list of (segment, mcb) pairs encountered. Stops early on a corrupt chain.
pub fn walk(bus: &Bus) -> Vec<(u16, Mcb)> {
    let mut out = Vec::new();
    let mut seg = FIRST_MCB_SEG;
    loop {
        if seg >= END_OF_CONVENTIONAL {
            break;
        }
        let m = read_mcb(bus, seg);
        if !m.is_valid() {
            break;
        }
        let last = m.is_last();
        let size = m.size;
        out.push((seg, m));
        if last {
            break;
        }
        // Advance one paragraph (header) + size paragraphs of data.
        seg = match seg.checked_add(1).and_then(|s| s.checked_add(size)) {
            Some(s) => s,
            None => break,
        };
    }
    out
}

/// Initialize an "idle" MCB chain: a single free block spanning all of
/// conventional memory. Used when the shell is loaded and no user process owns
/// anything yet.
pub fn init_empty(bus: &mut Bus) {
    let free_paras = END_OF_CONVENTIONAL - FIRST_MCB_SEG - 1;
    write_mcb(
        bus,
        FIRST_MCB_SEG,
        &Mcb {
            signature: MCB_Z,
            owner: FREE_OWNER,
            size: free_paras,
        },
    );
}

/// Build a fresh MCB chain consisting of one allocated block (the program)
/// followed by one free block covering the rest of conventional memory.
///
///   `program_start_seg` is the first usable paragraph of the program
///   (its PSP segment). `program_paras` is the total size of the program
///   block in paragraphs (PSP + image + anything below the initial heap).
pub fn init_for_program(bus: &mut Bus, program_start_seg: u16, program_paras: u16) {
    let prog_mcb_seg = program_start_seg.wrapping_sub(1);

    // No room after the program?
    let after_prog = program_start_seg.saturating_add(program_paras);
    if after_prog >= END_OF_CONVENTIONAL || after_prog + 1 > END_OF_CONVENTIONAL {
        // Program consumes the last paragraph — a single Z-block.
        write_mcb(
            bus,
            prog_mcb_seg,
            &Mcb {
                signature: MCB_Z,
                owner: program_start_seg,
                size: program_paras,
            },
        );
        return;
    }

    write_mcb(
        bus,
        prog_mcb_seg,
        &Mcb {
            signature: MCB_M,
            owner: program_start_seg,
            size: program_paras,
        },
    );

    let free_mcb_seg = after_prog;
    // free block covers everything from free_mcb_seg+1 up to END_OF_CONVENTIONAL
    let free_paras = END_OF_CONVENTIONAL - free_mcb_seg - 1;
    write_mcb(
        bus,
        free_mcb_seg,
        &Mcb {
            signature: MCB_Z,
            owner: FREE_OWNER,
            size: free_paras,
        },
    );
}

/// AH=48h: allocate `paras` paragraphs to `owner_psp`.
///
/// On success returns the first usable paragraph (one past the new MCB).
/// On failure returns the size of the largest free block (for BX), which is
/// what the BIOS returns when the caller asks for too much.
pub fn alloc(bus: &mut Bus, owner_psp: u16, paras: u16) -> Result<u16, u16> {
    // First-fit. Simple and matches real MS-DOS default strategy.
    let chain = walk(bus);

    let max_free = chain
        .iter()
        .filter(|(_, m)| m.is_free())
        .map(|(_, m)| m.size)
        .max()
        .unwrap_or(0);

    if paras == 0 || paras > max_free {
        return Err(max_free);
    }

    for (seg, m) in &chain {
        if !m.is_free() || m.size < paras {
            continue;
        }

        if m.size == paras {
            // Exact fit — flip ownership.
            write_mcb(
                bus,
                *seg,
                &Mcb {
                    signature: m.signature,
                    owner: owner_psp,
                    size: paras,
                },
            );
            return Ok(seg + 1);
        }

        // Split: shrink this block to `paras` and emit a new free block after
        // it that takes the remainder (minus 1 paragraph for its own header).
        let split_seg = seg + 1 + paras;
        let was_last = m.is_last();
        let remaining = m.size - paras - 1;

        write_mcb(
            bus,
            *seg,
            &Mcb {
                signature: MCB_M,
                owner: owner_psp,
                size: paras,
            },
        );
        write_mcb(
            bus,
            split_seg,
            &Mcb {
                signature: if was_last { MCB_Z } else { MCB_M },
                owner: FREE_OWNER,
                size: remaining,
            },
        );
        return Ok(seg + 1);
    }

    Err(max_free)
}

/// AH=49h: free the block that starts at `block_seg` (its MCB is at `block_seg - 1`).
///
/// Coalesces with adjacent free blocks on both sides.
pub fn free(bus: &mut Bus, block_seg: u16) -> Result<(), u8> {
    let mcb_seg = block_seg.wrapping_sub(1);
    let m = read_mcb(bus, mcb_seg);
    if !m.is_valid() {
        return Err(ERR_MCB_DESTROYED);
    }
    if m.is_free() {
        return Err(ERR_INVALID_BLOCK);
    }

    // Mark free.
    write_mcb(
        bus,
        mcb_seg,
        &Mcb {
            signature: m.signature,
            owner: FREE_OWNER,
            size: m.size,
        },
    );

    coalesce(bus, mcb_seg);
    Ok(())
}

/// AH=4Ah: resize the block at `block_seg` to `new_paras` paragraphs.
///
/// On failure returns the largest size (in paragraphs) that would have
/// succeeded, which the BIOS returns in BX.
pub fn resize(bus: &mut Bus, block_seg: u16, new_paras: u16) -> Result<(), u16> {
    let mcb_seg = block_seg.wrapping_sub(1);
    let m = read_mcb(bus, mcb_seg);
    if !m.is_valid() {
        return Err(0);
    }

    if new_paras == m.size {
        return Ok(());
    }

    if new_paras < m.size {
        // Shrink. Emit a new free block in the space we're giving up.
        let freed_header_seg = mcb_seg + 1 + new_paras;
        let freed_size = m.size - new_paras - 1;
        let was_last = m.is_last();

        write_mcb(
            bus,
            mcb_seg,
            &Mcb {
                signature: MCB_M,
                owner: m.owner,
                size: new_paras,
            },
        );
        write_mcb(
            bus,
            freed_header_seg,
            &Mcb {
                signature: if was_last { MCB_Z } else { MCB_M },
                owner: FREE_OWNER,
                size: freed_size,
            },
        );
        // Coalesce the newly freed block with what follows if that's free too.
        coalesce(bus, freed_header_seg);
        return Ok(());
    }

    // Grow. Need to consume the following free block (if any).
    if m.is_last() {
        return Err(m.size);
    }
    let next_seg = mcb_seg + 1 + m.size;
    let next = read_mcb(bus, next_seg);
    if !next.is_free() || !next.is_valid() {
        return Err(m.size);
    }
    let combined = m.size + 1 + next.size;
    if new_paras > combined {
        return Err(combined);
    }

    let was_next_last = next.is_last();

    if new_paras == combined {
        write_mcb(
            bus,
            mcb_seg,
            &Mcb {
                signature: if was_next_last { MCB_Z } else { MCB_M },
                owner: m.owner,
                size: new_paras,
            },
        );
    } else {
        let remaining = combined - new_paras - 1;
        let split_seg = mcb_seg + 1 + new_paras;

        write_mcb(
            bus,
            mcb_seg,
            &Mcb {
                signature: MCB_M,
                owner: m.owner,
                size: new_paras,
            },
        );
        write_mcb(
            bus,
            split_seg,
            &Mcb {
                signature: if was_next_last { MCB_Z } else { MCB_M },
                owner: FREE_OWNER,
                size: remaining,
            },
        );
    }

    Ok(())
}

/// Free every block owned by `psp`. Called when a process terminates so its
/// memory doesn't leak to the parent. No-op if the chain is corrupt.
pub fn free_owned_by(bus: &mut Bus, psp: u16) {
    if psp == 0 {
        return;
    }
    let chain = walk(bus);
    for (seg, m) in chain {
        if m.owner == psp {
            write_mcb(
                bus,
                seg,
                &Mcb {
                    signature: m.signature,
                    owner: FREE_OWNER,
                    size: m.size,
                },
            );
        }
    }
    // One more walk to coalesce adjacent frees.
    coalesce_all(bus);
}

/// Merge the free block at `mcb_seg` with its preceding and following free
/// neighbours (if any). Safe to call on an already-merged block.
fn coalesce(bus: &mut Bus, mcb_seg: u16) {
    // Forward: merge with next free block.
    let m = read_mcb(bus, mcb_seg);
    if m.is_valid() && m.is_free() && !m.is_last() {
        let next_seg = mcb_seg + 1 + m.size;
        let next = read_mcb(bus, next_seg);
        if next.is_valid() && next.is_free() {
            let merged_size = m.size + 1 + next.size;
            let new_sig = if next.is_last() { MCB_Z } else { MCB_M };
            write_mcb(
                bus,
                mcb_seg,
                &Mcb {
                    signature: new_sig,
                    owner: FREE_OWNER,
                    size: merged_size,
                },
            );
            // Clear out the absorbed header so walk() can't be fooled by it.
            write_mcb(
                bus,
                next_seg,
                &Mcb {
                    signature: 0,
                    owner: 0,
                    size: 0,
                },
            );
        }
    }

    // Backward: walk chain to find whether our predecessor is free and adjacent.
    let chain = walk(bus);
    for i in 1..chain.len() {
        let (prev_seg, prev) = chain[i - 1];
        let (curr_seg, _) = chain[i];
        if curr_seg == mcb_seg && prev.is_free() {
            let curr = read_mcb(bus, curr_seg);
            let merged = prev.size + 1 + curr.size;
            let new_sig = if curr.is_last() { MCB_Z } else { MCB_M };
            write_mcb(
                bus,
                prev_seg,
                &Mcb {
                    signature: new_sig,
                    owner: FREE_OWNER,
                    size: merged,
                },
            );
            write_mcb(
                bus,
                curr_seg,
                &Mcb {
                    signature: 0,
                    owner: 0,
                    size: 0,
                },
            );
            break;
        }
    }
}

/// Walk the chain and merge every run of adjacent free blocks.
fn coalesce_all(bus: &mut Bus) {
    // Iterate walk+merge until there's nothing left to merge.
    loop {
        let chain = walk(bus);
        let mut merged_any = false;
        for i in 0..chain.len().saturating_sub(1) {
            let (seg, m) = chain[i];
            let (next_seg, n) = chain[i + 1];
            if m.is_free() && n.is_free() && seg + 1 + m.size == next_seg {
                let merged = m.size + 1 + n.size;
                let new_sig = if n.is_last() { MCB_Z } else { MCB_M };
                write_mcb(
                    bus,
                    seg,
                    &Mcb {
                        signature: new_sig,
                        owner: FREE_OWNER,
                        size: merged,
                    },
                );
                write_mcb(
                    bus,
                    next_seg,
                    &Mcb {
                        signature: 0,
                        owner: 0,
                        size: 0,
                    },
                );
                merged_any = true;
                break;
            }
        }
        if !merged_any {
            break;
        }
    }
}
