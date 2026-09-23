//! XMS 3.0 driver (what HIMEM.SYS provides): extended memory in handles,
//! the HMA, and the A20 gate, for programs and DOS extenders.
//!
//! Programs find the driver with INT 2Fh AX=4300h and get its entry point
//! with AX=4310h, then far-call it with the function in AH. The entry is a
//! ROM stub (see bios.rs) running the `FE 39` service that lands in `call`.
//!
//! Extended memory blocks are carved out of RAM above the HMA (10FFF0h).
//! Locking a block returns its physical address, which is how DOS
//! extenders take the memory over for their protected-mode programs.

use crate::cpu::Cpu;

/// First byte of extended memory for blocks: the HMA's end, rounded up.
const XMS_BASE: u32 = 0x0011_0000;
/// Blocks are allocated in KB.
const KB: u32 = 1024;
/// Handles the driver can hand out.
const MAX_HANDLES: usize = 64;

/// XMS error codes (returned in BL).
const ERR_NOT_IMPLEMENTED: u8 = 0x80;
const ERR_HMA_IN_USE: u8 = 0x91;
const ERR_HMA_NOT_ALLOCATED: u8 = 0x93;
const ERR_OUT_OF_MEMORY: u8 = 0xA0;
const ERR_OUT_OF_HANDLES: u8 = 0xA1;
const ERR_INVALID_HANDLE: u8 = 0xA2;
const ERR_INVALID_SOURCE_HANDLE: u8 = 0xA3;
const ERR_INVALID_SOURCE_OFFSET: u8 = 0xA4;
const ERR_INVALID_DEST_HANDLE: u8 = 0xA5;
const ERR_INVALID_DEST_OFFSET: u8 = 0xA6;
const ERR_INVALID_LENGTH: u8 = 0xA7;
const ERR_NOT_LOCKED: u8 = 0xAA;
const ERR_LOCKED: u8 = 0xAB;
const ERR_NO_UMB: u8 = 0xB1;
const ERR_INVALID_UMB: u8 = 0xB2;

#[derive(Clone, Copy, Debug)]
struct Block {
    base: u32,
    /// Size in KB.
    size_kb: u32,
    locks: u8,
}

#[derive(Default)]
pub struct Xms {
    /// Blocks by handle - 1.
    blocks: Vec<Option<Block>>,
    hma_allocated: bool,
    /// Nested local A20 enables (functions 05h/06h).
    a20_local: u32,
    /// Global A20 enable (functions 03h/04h).
    a20_global: bool,
}

impl Xms {
    pub fn new() -> Self {
        Self::default()
    }

    /// The allocated handles: (handle, base address, size in KB, lock
    /// count), for debuggers.
    pub fn handles(&self) -> Vec<(u16, u32, u32, u8)> {
        self.blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.map(|b| (i as u16 + 1, b.base, b.size_kb, b.locks)))
            .collect()
    }

    pub fn hma_allocated(&self) -> bool {
        self.hma_allocated
    }

    fn block(&self, handle: u16) -> Option<&Block> {
        self.blocks.get((handle as usize).wrapping_sub(1))?.as_ref()
    }

    fn block_mut(&mut self, handle: u16) -> Option<&mut Block> {
        self.blocks.get_mut((handle as usize).wrapping_sub(1))?.as_mut()
    }

    fn free_handles(&self) -> usize {
        MAX_HANDLES - self.blocks.iter().filter(|b| b.is_some()).count()
    }

    /// Free gaps of extended memory as (base, size in KB), in address order.
    fn gaps(&self, memory_end: u32) -> Vec<(u32, u32)> {
        let mut used: Vec<(u32, u32)> = self
            .blocks
            .iter()
            .flatten()
            .map(|b| (b.base, b.base + b.size_kb * KB))
            .collect();
        used.sort();
        let mut gaps = Vec::new();
        let mut at = XMS_BASE;
        for (start, end) in used {
            if start > at {
                gaps.push((at, (start - at) / KB));
            }
            at = at.max(end);
        }
        if memory_end > at {
            gaps.push((at, (memory_end - at) / KB));
        }
        gaps
    }

    /// Largest free block and total free memory, in KB.
    fn free_kb(&self, memory_end: u32) -> (u32, u32) {
        let gaps = self.gaps(memory_end);
        let largest = gaps.iter().map(|&(_, kb)| kb).max().unwrap_or(0);
        let total = gaps.iter().map(|&(_, kb)| kb).sum();
        (largest, total)
    }

    /// Allocate `size_kb` KB. Returns the handle.
    fn allocate(&mut self, size_kb: u32, memory_end: u32) -> Result<u16, u8> {
        if self.free_handles() == 0 {
            return Err(ERR_OUT_OF_HANDLES);
        }
        let base = if size_kb == 0 {
            XMS_BASE
        } else {
            self.gaps(memory_end)
                .into_iter()
                .find(|&(_, kb)| kb >= size_kb)
                .map(|(base, _)| base)
                .ok_or(ERR_OUT_OF_MEMORY)?
        };
        let block = Block { base, size_kb, locks: 0 };
        let slot = match self.blocks.iter().position(|b| b.is_none()) {
            Some(i) => {
                self.blocks[i] = Some(block);
                i
            }
            None => {
                self.blocks.push(Some(block));
                self.blocks.len() - 1
            }
        };
        Ok(slot as u16 + 1)
    }
}

/// Report success: AX=1.
fn ok(cpu: &mut Cpu) {
    cpu.set_ax(1);
}

/// Report failure: AX=0, error code in BL.
fn fail(cpu: &mut Cpu, error: u8) {
    cpu.set_ax(0);
    cpu.set_bx((cpu.bx() & 0xFF00) | error as u16);
}

/// End of the memory XMS can hand out.
fn memory_end(cpu: &Cpu) -> u32 {
    cpu.bus.ram().len() as u32
}

/// Apply the local and global enables to the A20 gate.
fn update_a20(cpu: &mut Cpu) {
    let xms = &cpu.bus.xms;
    let open = xms.a20_global || xms.a20_local > 0;
    cpu.bus.set_a20(open);
}

/// The driver entry point: the function is in AH.
pub fn call(cpu: &mut Cpu) {
    let function = cpu.get_ah();
    let end = memory_end(cpu);
    match function {
        // Get version: XMS 3.0, driver 3.10, HMA present.
        0x00 => {
            cpu.set_ax(0x0300);
            cpu.set_bx(0x0310);
            cpu.set_dx(0x0001);
        }
        0x01 => {
            if cpu.bus.xms.hma_allocated {
                fail(cpu, ERR_HMA_IN_USE);
            } else {
                cpu.bus.xms.hma_allocated = true;
                ok(cpu);
            }
        }
        0x02 => {
            if cpu.bus.xms.hma_allocated {
                cpu.bus.xms.hma_allocated = false;
                ok(cpu);
            } else {
                fail(cpu, ERR_HMA_NOT_ALLOCATED);
            }
        }
        0x03 => {
            cpu.bus.xms.a20_global = true;
            update_a20(cpu);
            ok(cpu);
        }
        0x04 => {
            cpu.bus.xms.a20_global = false;
            update_a20(cpu);
            ok(cpu);
        }
        0x05 => {
            cpu.bus.xms.a20_local += 1;
            update_a20(cpu);
            ok(cpu);
        }
        0x06 => {
            cpu.bus.xms.a20_local = cpu.bus.xms.a20_local.saturating_sub(1);
            update_a20(cpu);
            ok(cpu);
        }
        0x07 => {
            let a20 = cpu.bus.a20() as u16;
            cpu.set_ax(a20);
            cpu.set_bx(cpu.bx() & 0xFF00);
        }
        // Query free memory: largest block and total, in KB.
        0x08 => {
            let (largest, total) = cpu.bus.xms.free_kb(end);
            cpu.set_ax(largest.min(0xFFFF) as u16);
            cpu.set_dx(total.min(0xFFFF) as u16);
            cpu.set_bx(cpu.bx() & 0xFF00 | if total == 0 { ERR_OUT_OF_MEMORY as u16 } else { 0 });
        }
        0x88 => {
            let (largest, total) = cpu.bus.xms.free_kb(end);
            cpu.set_eax(largest);
            cpu.set_edx(total);
            cpu.set_ecx(end - 1);
            cpu.set_bx(cpu.bx() & 0xFF00 | if total == 0 { ERR_OUT_OF_MEMORY as u16 } else { 0 });
        }
        0x09 | 0x89 => {
            let size_kb = if function == 0x09 { cpu.dx() as u32 } else { cpu.edx() };
            match cpu.bus.xms.allocate(size_kb, end) {
                Ok(handle) => {
                    cpu.set_dx(handle);
                    ok(cpu);
                }
                Err(e) => {
                    cpu.set_dx(0);
                    fail(cpu, e);
                }
            }
        }
        0x0A => {
            let handle = cpu.dx();
            match cpu.bus.xms.block(handle) {
                None => fail(cpu, ERR_INVALID_HANDLE),
                Some(b) if b.locks > 0 => fail(cpu, ERR_LOCKED),
                Some(_) => {
                    cpu.bus.xms.blocks[handle as usize - 1] = None;
                    ok(cpu);
                }
            }
        }
        0x0B => move_block(cpu),
        // Lock: the block's physical address in DX:BX.
        0x0C => {
            let handle = cpu.dx();
            match cpu.bus.xms.block_mut(handle) {
                None => fail(cpu, ERR_INVALID_HANDLE),
                Some(b) => {
                    b.locks = b.locks.saturating_add(1);
                    let base = b.base;
                    cpu.set_dx((base >> 16) as u16);
                    cpu.set_bx(base as u16);
                    ok(cpu);
                }
            }
        }
        0x0D => {
            let handle = cpu.dx();
            match cpu.bus.xms.block_mut(handle) {
                None => fail(cpu, ERR_INVALID_HANDLE),
                Some(b) if b.locks == 0 => fail(cpu, ERR_NOT_LOCKED),
                Some(b) => {
                    b.locks -= 1;
                    ok(cpu);
                }
            }
        }
        // Handle information: lock count, free handles, size in KB.
        0x0E | 0x8E => {
            let handle = cpu.dx();
            let free = cpu.bus.xms.free_handles();
            match cpu.bus.xms.block(handle).copied() {
                None => fail(cpu, ERR_INVALID_HANDLE),
                Some(b) => {
                    if function == 0x0E {
                        cpu.set_bx(((b.locks as u16) << 8) | free.min(0xFF) as u16);
                        cpu.set_dx(b.size_kb.min(0xFFFF) as u16);
                    } else {
                        cpu.set_bx((b.locks as u16) << 8);
                        cpu.set_cx(free as u16);
                        cpu.set_edx(b.size_kb);
                    }
                    ok(cpu);
                }
            }
        }
        0x0F | 0x8F => {
            let handle = cpu.dx();
            let size_kb = if function == 0x0F { cpu.bx() as u32 } else { cpu.ebx() };
            match resize(cpu, handle, size_kb) {
                Ok(()) => ok(cpu),
                Err(e) => fail(cpu, e),
            }
        }
        // Upper memory blocks: none.
        0x10 => {
            cpu.set_dx(0);
            fail(cpu, ERR_NO_UMB);
        }
        0x11 | 0x12 => fail(cpu, ERR_INVALID_UMB),
        _ => {
            cpu.bus
                .log_string(&format!("[XMS] Unknown function AH={:02X}", function));
            fail(cpu, ERR_NOT_IMPLEMENTED);
        }
    }
}

/// Grow or shrink an unlocked block, moving it (with its contents) if it
/// can't grow in place.
fn resize(cpu: &mut Cpu, handle: u16, size_kb: u32) -> Result<(), u8> {
    let end = memory_end(cpu);
    let block = *cpu.bus.xms.block(handle).ok_or(ERR_INVALID_HANDLE)?;
    if block.locks > 0 {
        return Err(ERR_LOCKED);
    }
    // Take the block out, then find room for the new size as if it were
    // free: in place if the free space around it is big enough, otherwise
    // in the first gap that fits.
    cpu.bus.xms.blocks[handle as usize - 1] = None;
    let gaps = cpu.bus.xms.gaps(end);
    let new_end = block.base as u64 + (size_kb * KB) as u64;
    let in_place = gaps.iter().any(|&(base, kb)| {
        base <= block.base && (base + kb * KB) as u64 >= new_end
    });
    let base = if in_place {
        Some(block.base)
    } else {
        gaps.iter().find(|&&(_, kb)| kb >= size_kb).map(|&(base, _)| base)
    };
    let Some(base) = base else {
        cpu.bus.xms.blocks[handle as usize - 1] = Some(block);
        return Err(ERR_OUT_OF_MEMORY);
    };
    if base != block.base {
        let len = (block.size_kb.min(size_kb) * KB) as usize;
        let data: Vec<u8> = (0..len)
            .map(|i| cpu.bus.read_8(block.base as usize + i))
            .collect();
        cpu.bus.load_bytes(base as usize, &data);
    }
    cpu.bus.xms.blocks[handle as usize - 1] = Some(Block {
        base,
        size_kb,
        locks: 0,
    });
    Ok(())
}

/// Function 0Bh: copy between extended memory blocks and conventional
/// memory, as described by the structure at DS:SI.
fn move_block(cpu: &mut Cpu) {
    let params = cpu.get_physical_addr(cpu.ds(), cpu.si());
    let bus = &cpu.bus;
    let length = bus.read_32(params);
    let src_handle = bus.read_16(params + 4);
    let src_offset = bus.read_32(params + 6);
    let dst_handle = bus.read_16(params + 10);
    let dst_offset = bus.read_32(params + 12);

    if length % 2 != 0 {
        fail(cpu, ERR_INVALID_LENGTH);
        return;
    }
    // Resolve a (handle, offset) pair to a physical address. Handle 0
    // means the offset is a real-mode segment:offset pointer.
    let resolve = |handle: u16, offset: u32, bad_handle: u8, bad_offset: u8| -> Result<usize, u8> {
        if handle == 0 {
            let segment = offset >> 16;
            let off = offset & 0xFFFF;
            return Ok(((segment << 4) + off) as usize);
        }
        let block = cpu.bus.xms.block(handle).ok_or(bad_handle)?;
        if offset as u64 + length as u64 > (block.size_kb * KB) as u64 {
            return Err(bad_offset);
        }
        Ok((block.base + offset) as usize)
    };
    let src = resolve(src_handle, src_offset, ERR_INVALID_SOURCE_HANDLE, ERR_INVALID_SOURCE_OFFSET);
    let dst = resolve(dst_handle, dst_offset, ERR_INVALID_DEST_HANDLE, ERR_INVALID_DEST_OFFSET);
    match (src, dst) {
        (Ok(src), Ok(dst)) => {
            let data: Vec<u8> = (0..length as usize).map(|i| cpu.bus.read_8(src + i)).collect();
            for (i, &b) in data.iter().enumerate() {
                cpu.bus.write_8(dst + i, b);
            }
            ok(cpu);
        }
        (Err(e), _) | (_, Err(e)) => fail(cpu, e),
    }
}
