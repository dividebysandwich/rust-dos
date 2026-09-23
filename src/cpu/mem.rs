//! Memory access by instructions: segment limit checks, linear to physical
//! translation, and the stack.
//!
//! Instructions access memory in two steps: `mem_ref` checks the access and
//! can fault; `mem_read` and `mem_write` then cannot. A read-modify-write
//! instruction checks its destination for writing before it reads it, so
//! nothing is written by an instruction that later faults.

use super::fault::{CpuResult, Fault};
use super::regs::ATTR_DB;
use super::{Cpu, Seg};

/// Kind of memory access, for protection checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// A memory operand whose access has been checked: its bytes can be read
/// and written without faulting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemRef {
    pub lin: u32,
    pub phys: u32,
    pub size: u8,
}

/// Physical address lines: 20 while there is 1 MB of RAM and no A20 gate.
const PHYS_MASK: u32 = 0xF_FFFF;

/// The fault a segment limit violation raises: #SS for the stack segment,
/// #GP for the others.
#[inline(always)]
fn limit_fault(seg: Seg) -> Fault {
    if seg == Seg::SS { Fault::ss(0) } else { Fault::gp(0) }
}

impl Cpu {
    /// Physical address of a linear address.
    #[inline(always)]
    pub fn translate(&self, lin: u32) -> u32 {
        lin & PHYS_MASK
    }

    /// Linear address of an access of `size` bytes at `seg:off`, checked
    /// against the segment limit.
    #[inline(always)]
    pub fn seg_linear(&self, seg: Seg, off: u32, size: u8) -> CpuResult<u32> {
        let cache = &self.seg[seg as usize];
        let last = off.wrapping_add(size as u32 - 1);
        if last > cache.limit || last < off {
            return Err(limit_fault(seg));
        }
        Ok(cache.base.wrapping_add(off))
    }

    /// Check an access of `size` bytes at `seg:off`.
    #[inline(always)]
    pub fn mem_ref(&mut self, seg: Seg, off: u32, size: u8, _access: Access) -> CpuResult<MemRef> {
        let lin = self.seg_linear(seg, off, size)?;
        Ok(MemRef {
            lin,
            phys: self.translate(lin),
            size,
        })
    }

    /// Read a checked operand (zero-extended).
    #[inline(always)]
    pub fn mem_read(&self, r: MemRef) -> u32 {
        if r.phys & 0xFFF <= 0x1000 - r.size as u32 {
            // Within one page: physically contiguous.
            let p = r.phys as usize;
            match r.size {
                1 => self.bus.read_8(p) as u32,
                2 => self.bus.read_16(p) as u32,
                _ => self.bus.read_32(p),
            }
        } else {
            let mut value = 0;
            for i in 0..r.size as u32 {
                let p = self.translate(r.lin.wrapping_add(i)) as usize;
                value |= (self.bus.read_8(p) as u32) << (8 * i);
            }
            value
        }
    }

    /// Write a checked operand (the low `size` bytes of `value`).
    #[inline(always)]
    pub fn mem_write(&mut self, r: MemRef, value: u32) {
        if r.phys & 0xFFF <= 0x1000 - r.size as u32 {
            let p = r.phys as usize;
            match r.size {
                1 => {
                    self.bus.write_8(p, value as u8);
                }
                2 => {
                    self.bus.write_16(p, value as u16);
                }
                _ => {
                    self.bus.write_32(p, value);
                }
            }
        } else {
            for i in 0..r.size as u32 {
                let p = self.translate(r.lin.wrapping_add(i)) as usize;
                self.bus.write_8(p, (value >> (8 * i)) as u8);
            }
        }
    }

    pub fn read_u8(&mut self, seg: Seg, off: u32) -> CpuResult<u8> {
        let r = self.mem_ref(seg, off, 1, Access::Read)?;
        Ok(self.mem_read(r) as u8)
    }

    pub fn read_u16(&mut self, seg: Seg, off: u32) -> CpuResult<u16> {
        let r = self.mem_ref(seg, off, 2, Access::Read)?;
        Ok(self.mem_read(r) as u16)
    }

    pub fn read_u32(&mut self, seg: Seg, off: u32) -> CpuResult<u32> {
        let r = self.mem_ref(seg, off, 4, Access::Read)?;
        Ok(self.mem_read(r))
    }

    /// Read `size` (1, 2 or 4) bytes.
    pub fn read_sized(&mut self, seg: Seg, off: u32, size: u8) -> CpuResult<u32> {
        let r = self.mem_ref(seg, off, size, Access::Read)?;
        Ok(self.mem_read(r))
    }

    pub fn write_u8(&mut self, seg: Seg, off: u32, value: u8) -> CpuResult {
        let r = self.mem_ref(seg, off, 1, Access::Write)?;
        self.mem_write(r, value as u32);
        Ok(())
    }

    pub fn write_u16(&mut self, seg: Seg, off: u32, value: u16) -> CpuResult {
        let r = self.mem_ref(seg, off, 2, Access::Write)?;
        self.mem_write(r, value as u32);
        Ok(())
    }

    pub fn write_u32(&mut self, seg: Seg, off: u32, value: u32) -> CpuResult {
        let r = self.mem_ref(seg, off, 4, Access::Write)?;
        self.mem_write(r, value);
        Ok(())
    }

    /// Read a word at a linear address, as the CPU reads the interrupt
    /// vector table.
    pub fn read_linear_u16(&self, lin: u32) -> u16 {
        let lo = self.bus.read_8(self.translate(lin) as usize) as u16;
        let hi = self.bus.read_8(self.translate(lin.wrapping_add(1)) as usize) as u16;
        lo | (hi << 8)
    }

    /// True when the stack segment is 32-bit (SS.B): the stack pointer is
    /// ESP rather than SP.
    #[inline(always)]
    pub fn stack32(&self) -> bool {
        self.seg[Seg::SS as usize].attr & ATTR_DB != 0
    }

    /// The stack pointer as the stack segment uses it: ESP, or SP.
    #[inline(always)]
    pub fn stack_ptr(&self) -> u32 {
        if self.stack32() { self.esp() } else { self.sp() as u32 }
    }

    /// Set the stack pointer as the stack segment uses it. A 16-bit stack
    /// only changes SP.
    #[inline(always)]
    pub fn set_stack_ptr(&mut self, value: u32) {
        if self.stack32() {
            self.set_esp(value);
        } else {
            self.set_sp(value as u16);
        }
    }

    /// Stack pointer `delta` bytes away from the current one, wrapped to the
    /// stack width.
    #[inline(always)]
    fn stack_offset(&self, delta: u32) -> u32 {
        let sp = self.stack_ptr().wrapping_add(delta);
        if self.stack32() { sp } else { sp & 0xFFFF }
    }

    /// Push `size` (2 or 4) bytes of `value`.
    #[inline(always)]
    pub fn push_sized(&mut self, size: u8, value: u32) -> CpuResult {
        let sp = self.stack_offset((size as u32).wrapping_neg());
        let r = self.mem_ref(Seg::SS, sp, size, Access::Write)?;
        self.mem_write(r, value);
        self.set_stack_ptr(sp);
        Ok(())
    }

    /// Pop `size` (2 or 4) bytes.
    #[inline(always)]
    pub fn pop_sized(&mut self, size: u8) -> CpuResult<u32> {
        let value = self.stack_read(0, size)?;
        let sp = self.stack_offset(size as u32);
        self.set_stack_ptr(sp);
        Ok(value)
    }

    /// Read `size` bytes `depth` bytes above the top of the stack without
    /// popping them.
    pub fn stack_read(&mut self, depth: u32, size: u8) -> CpuResult<u32> {
        let sp = self.stack_offset(depth);
        let r = self.mem_ref(Seg::SS, sp, size, Access::Read)?;
        Ok(self.mem_read(r))
    }

    /// Write `size` bytes `depth` bytes above the stack pointer as it will be
    /// after `total` bytes are pushed, for instructions that push several
    /// values and move the stack pointer once.
    pub fn stack_write_below(&mut self, total: u32, depth: u32, size: u8, value: u32) -> CpuResult {
        let sp = self.stack_offset(depth.wrapping_sub(total));
        let r = self.mem_ref(Seg::SS, sp, size, Access::Write)?;
        self.mem_write(r, value);
        Ok(())
    }

    /// Push a word for emulator services (interrupt frames built by HLE
    /// code). They run in real mode on the program's stack, where a push
    /// can only fault if the program's stack pointer is broken already.
    pub fn push(&mut self, value: u16) {
        if let Err(fault) = self.push_sized(2, value as u32) {
            self.bus.log_string(&format!(
                "[CPU] Stack fault {:?} pushing at SS:SP={:04X}:{:04X}",
                fault,
                self.ss(),
                self.sp()
            ));
        }
    }

    /// Pop a word for emulator services, see `push`.
    pub fn pop(&mut self) -> u16 {
        match self.pop_sized(2) {
            Ok(value) => value as u16,
            Err(fault) => {
                self.bus.log_string(&format!(
                    "[CPU] Stack fault {:?} popping at SS:SP={:04X}:{:04X}",
                    fault,
                    self.ss(),
                    self.sp()
                ));
                0
            }
        }
    }
}
