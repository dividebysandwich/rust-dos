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
//!
//! With upper memory (`Umb`), as DOS 5 with a UMB provider has it, the last
//! paragraph of conventional memory holds an MCB owned by DOS ("SC") that
//! covers the adapters' memory up to D000h, where the upper memory blocks
//! go on. Unlinked, conventional memory's last block is a 'Z' and the upper
//! blocks are a chain of their own; linked (INT 21h AX=5803h), it is an 'M'
//! and one chain runs through both.

use crate::bus::Bus;
use crate::video::adapter::Adapter;

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
/// The owner of DOS's own blocks.
pub const DOS_OWNER: u16 = 0x0008;

/// With upper memory, the MCB in the last paragraph of conventional memory
/// that covers the memory up to the upper memory blocks.
pub const UMB_COVER_SEG: u16 = 0x9FFF;
/// On a Tandy 1000: where conventional memory's blocks end, 624 KB. The
/// top 16 KB is the video memory the 16 KB modes show, and the 32 KB modes
/// reach 16 KB below it, as on a real Tandy.
pub const TANDY_END: u16 = 0x9C00;
/// On a PCjr: the MCB of the first block programs can have. DOS keeps the
/// memory below, which holds the video memory at 18000h-1FFFFh and 16 KB
/// above it that Space Quest 1.0x needs, as DOSBox's expanded PCjr memory
/// has it.
pub const PCJR_FIRST_FREE: u16 = 0x2400;

/// Where conventional memory ends on this machine (without upper memory's
/// cover block): A0000h, or 9C000h on a Tandy.
pub fn conventional_end(bus: &Bus) -> u16 {
    if bus.vga.adapter == Adapter::Tandy { TANDY_END } else { END_OF_CONVENTIONAL }
}

/// The MCB in conventional memory's last paragraph that covers the memory
/// up to the upper memory blocks.
pub fn umb_cover_seg(bus: &Bus) -> u16 {
    conventional_end(bus) - 1
}

/// The MCB of the first block programs can have: the first MCB, or on a
/// PCjr the one after the block DOS keeps over the video memory.
pub fn first_free(bus: &Bus) -> u16 {
    if bus.vga.adapter == Adapter::Pcjr { PCJR_FIRST_FREE } else { FIRST_MCB_SEG }
}

/// The first upper memory block's MCB, and where upper memory ends.
pub const UMB_START: u16 = 0xD000;
pub const UMB_END: u16 = 0xF000;

/// Upper memory: its size in paragraphs from `UMB_START` on, and whether it
/// is linked to conventional memory's chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Umb {
    pub size: u16,
    pub linked: bool,
}

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

/// Where conventional memory's blocks end: the paragraph of the upper
/// memory's cover MCB, or the end of conventional memory without upper
/// memory.
pub fn low_end(bus: &Bus) -> u16 {
    if bus.umb.is_some() { umb_cover_seg(bus) } else { conventional_end(bus) }
}

/// Walk the MCB chain from FIRST_MCB_SEG to the 'Z' sentinel. Returns the
/// list of (segment, mcb) pairs encountered. Stops early on a corrupt chain.
/// With upper memory linked, the chain goes on through it.
pub fn walk(bus: &Bus) -> Vec<(u16, Mcb)> {
    walk_from(bus, FIRST_MCB_SEG)
}

/// The upper memory blocks' chain, from `UMB_START`: empty without upper
/// memory.
pub fn walk_upper(bus: &Bus) -> Vec<(u16, Mcb)> {
    if bus.umb.is_none() {
        return Vec::new();
    }
    walk_from(bus, UMB_START)
}

/// Every block, in conventional and upper memory, linked or not.
pub fn walk_all(bus: &Bus) -> Vec<(u16, Mcb)> {
    let mut chain = walk(bus);
    if bus.umb.is_some_and(|u| !u.linked) {
        chain.extend(walk_upper(bus));
    }
    chain
}

/// The chain the block whose MCB is at `seg` is in.
fn chain_of(bus: &Bus, seg: u16) -> Vec<(u16, Mcb)> {
    if bus.umb.is_some() && seg >= UMB_START { walk_upper(bus) } else { walk(bus) }
}

fn walk_from(bus: &Bus, start: u16) -> Vec<(u16, Mcb)> {
    let limit = if bus.umb.is_some() { UMB_END } else { conventional_end(bus) };
    let mut out = Vec::new();
    let mut seg = start;
    loop {
        if seg >= limit {
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
    let first = first_free(bus);
    if first > FIRST_MCB_SEG {
        // The PCjr's: DOS keeps the memory up to it.
        write_mcb(bus, FIRST_MCB_SEG, &Mcb { signature: MCB_M, owner: DOS_OWNER, size: first - FIRST_MCB_SEG - 1 });
        for (i, &b) in b"SC".iter().enumerate() {
            bus.write_8(header_addr(FIRST_MCB_SEG) + 8 + i, b);
        }
    }
    let free_paras = low_end(bus) - first - 1;
    write_mcb(
        bus,
        first,
        &Mcb {
            signature: MCB_Z,
            owner: FREE_OWNER,
            size: free_paras,
        },
    );
}

/// Free everything from the MCB at `seg` up: `seg` becomes one free 'Z'
/// block reaching the end of conventional memory. Blocks below `seg` (the
/// resident TSRs) are kept, and free blocks directly below it merge into the
/// new free block. Returns the MCB segment of that trailing free block,
/// which is below `seg` when the TSRs have freed their memory, or None when
/// the chain is corrupt and doesn't reach `seg`.
pub fn release_from(bus: &mut Bus, seg: u16) -> Option<u16> {
    if seg <= first_free(bus) {
        init_empty(bus);
        return Some(first_free(bus));
    }
    let end = low_end(bus);
    let chain = walk(bus);
    let &(below, m) = chain.iter().take_while(|(s, _)| *s < seg).last()?;
    let below_end = below as u32 + 1 + m.size as u32;
    let free_from = if below_end == seg as u32 && seg < end {
        write_mcb(
            bus,
            below,
            &Mcb {
                signature: MCB_M,
                ..m
            },
        );
        seg
    } else if m.is_free() && below_end > seg as u32 {
        // Already free across `seg`, e.g. a TSR that released itself.
        below
    } else {
        return None;
    };
    write_mcb(
        bus,
        free_from,
        &Mcb {
            signature: MCB_Z,
            owner: FREE_OWNER,
            size: end - free_from - 1,
        },
    );
    coalesce_all(bus);
    walk(bus).last().map(|&(s, _)| s)
}

/// Set up upper memory as `bus.umb` has it: the cover MCB, and one free
/// block over all of it. Conventional memory's chain is left to reach the
/// cover MCB (see `release_from`).
pub fn build_upper(bus: &mut Bus) {
    let Some(umb) = bus.umb else { return };
    let cover = umb_cover_seg(bus);
    write_mcb(bus, cover, &Mcb { signature: MCB_M, owner: DOS_OWNER, size: UMB_START - cover - 1 });
    for (i, &b) in b"SC".iter().enumerate() {
        bus.write_8(header_addr(cover) + 8 + i, b);
    }
    write_mcb(bus, UMB_START, &Mcb { signature: MCB_Z, owner: FREE_OWNER, size: umb.size - 1 });
    bus.umb = Some(Umb { linked: false, ..umb });
}

/// Link upper memory to conventional memory's chain, or unlink it
/// (INT 21h AX=5803h): conventional memory's last block becomes an 'M' or
/// a 'Z'. Fails without upper memory or with a broken chain.
pub fn link_upper(bus: &mut Bus, on: bool) -> Result<(), ()> {
    let Some(umb) = bus.umb else { return Err(()) };
    if umb.linked == on {
        return Ok(());
    }
    let chain = walk(bus);
    let cover = umb_cover_seg(bus);
    let &(last, m) = chain.iter().take_while(|(s, _)| *s < cover).last().ok_or(())?;
    if last as u32 + 1 + m.size as u32 != cover as u32 {
        return Err(());
    }
    write_mcb(bus, last, &Mcb { signature: if on { MCB_M } else { MCB_Z }, ..m });
    bus.umb = Some(Umb { linked: on, ..umb });
    crate::dos_data::set_upper_linked(bus, on);
    Ok(())
}

/// Free the upper memory blocks of every owner but `keep` (the resident
/// programs loaded high). False if the upper chain is broken, which is
/// then set up afresh.
pub fn release_upper(bus: &mut Bus, keep: &[u16]) -> bool {
    if bus.umb.is_none() {
        return true;
    }
    let chain = walk_upper(bus);
    let whole = chain.last().is_some_and(|(s, m)| m.is_last() && *s as u32 + 1 + m.size as u32 <= UMB_END as u32);
    if !whole {
        build_upper(bus);
        return false;
    }
    for (seg, m) in chain {
        if !m.is_free() && !keep.contains(&m.owner) {
            write_mcb(bus, seg, &Mcb { owner: FREE_OWNER, ..m });
        }
    }
    coalesce_chain(bus, UMB_START);
    true
}

/// The largest free upper memory block: its MCB's segment and its size.
pub fn largest_free_upper(bus: &Bus) -> Option<(u16, u16)> {
    walk_upper(bus).into_iter().filter(|(_, m)| m.is_free()).max_by_key(|(_, m)| m.size).map(|(s, m)| (s, m.size))
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
    let end = low_end(bus);
    let after_prog = program_start_seg.saturating_add(program_paras);
    if after_prog >= end || after_prog + 1 > end {
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
    // free block covers everything from free_mcb_seg+1 up to the end of
    // conventional memory
    let free_paras = end - free_mcb_seg - 1;
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

/// How AH=48h picks among the free blocks that are large enough (AH=58h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The lowest one.
    First,
    /// The smallest one.
    Best,
    /// The highest one, allocating from its top end.
    Last,
}

impl Fit {
    /// The fit of an AH=58h strategy code (its low bits; the UMB bits are
    /// for `alloc_strategy`).
    pub fn from_strategy(strategy: u16) -> Fit {
        match strategy & 0x3F {
            0 => Fit::First,
            1 => Fit::Best,
            _ => Fit::Last,
        }
    }
}

/// AH=48h: allocate `paras` paragraphs to `owner_psp`, first fit.
///
/// On success returns the first usable paragraph (one past the new MCB).
/// On failure returns the size of the largest free block (for BX), which is
/// what the BIOS returns when the caller asks for too much.
pub fn alloc(bus: &mut Bus, owner_psp: u16, paras: u16) -> Result<u16, u16> {
    alloc_fit(bus, owner_psp, paras, Fit::First)
}

/// AH=48h with the allocation strategy `strategy` (AH=58h): its fit, and
/// its upper memory bits, 40h for upper memory only and 80h for upper
/// memory first, whether it is linked or not.
pub fn alloc_strategy(bus: &mut Bus, owner_psp: u16, paras: u16, strategy: u16) -> Result<u16, u16> {
    let fit = Fit::from_strategy(strategy);
    if strategy & 0xC0 != 0 && bus.umb.is_some() {
        let chain = walk_upper(bus);
        match alloc_in(bus, chain, owner_psp, paras, fit) {
            Ok(seg) => return Ok(seg),
            Err(largest) if strategy & 0x40 != 0 => return Err(largest),
            Err(largest) => {
                let chain = walk(bus);
                return alloc_in(bus, chain, owner_psp, paras, fit).map_err(|low| low.max(largest));
            }
        }
    }
    alloc_fit(bus, owner_psp, paras, fit)
}

/// AH=48h with the allocation strategy `fit`.
pub fn alloc_fit(bus: &mut Bus, owner_psp: u16, paras: u16, fit: Fit) -> Result<u16, u16> {
    let chain = walk(bus);
    alloc_in(bus, chain, owner_psp, paras, fit)
}

/// Allocate from the free blocks of `chain`.
fn alloc_in(bus: &mut Bus, chain: Vec<(u16, Mcb)>, owner_psp: u16, paras: u16, fit: Fit) -> Result<u16, u16> {

    let max_free = chain
        .iter()
        .filter(|(_, m)| m.is_free())
        .map(|(_, m)| m.size)
        .max()
        .unwrap_or(0);

    if paras == 0 || paras > max_free {
        return Err(max_free);
    }

    let mut fitting = chain.iter().filter(|(_, m)| m.is_free() && m.size >= paras);
    let chosen = match fit {
        Fit::First => fitting.next(),
        Fit::Best => fitting.min_by_key(|(_, m)| m.size),
        Fit::Last => fitting.last(),
    };
    let Some(&(seg, m)) = chosen else {
        return Err(max_free);
    };

    if m.size == paras {
        // Exact fit — flip ownership.
        write_mcb(bus, seg, &Mcb { signature: m.signature, owner: owner_psp, size: paras });
        return Ok(seg + 1);
    }

    // Split, leaving the remainder (minus 1 paragraph for the second
    // header) free: after the new block, or before it for a last fit.
    let remaining = m.size - paras - 1;
    let last_signature = if m.is_last() { MCB_Z } else { MCB_M };
    if fit == Fit::Last {
        let block = seg + 1 + remaining;
        write_mcb(bus, seg, &Mcb { signature: MCB_M, owner: FREE_OWNER, size: remaining });
        write_mcb(bus, block, &Mcb { signature: last_signature, owner: owner_psp, size: paras });
        Ok(block + 1)
    } else {
        write_mcb(bus, seg, &Mcb { signature: MCB_M, owner: owner_psp, size: paras });
        write_mcb(bus, seg + 1 + paras, &Mcb { signature: last_signature, owner: FREE_OWNER, size: remaining });
        Ok(seg + 1)
    }
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
    let chain = walk_all(bus);
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
    let chain = chain_of(bus, mcb_seg);
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

/// Walk the chains and merge every run of adjacent free blocks.
fn coalesce_all(bus: &mut Bus) {
    coalesce_chain(bus, FIRST_MCB_SEG);
    if bus.umb.is_some() {
        coalesce_chain(bus, UMB_START);
    }
}

/// Merge every run of adjacent free blocks of the chain from `start`.
fn coalesce_chain(bus: &mut Bus, start: u16) {
    // Iterate walk+merge until there's nothing left to merge.
    loop {
        let chain = walk_from(bus, start);
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

// Whether there is upper memory comes with the configuration, and with it
// its size; programs link it and unlink it.
crate::state_fields!(Umb { linked } skip { size });
