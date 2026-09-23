//! Memory access by instructions: segment limit and type checks, linear to
//! physical translation, and the stack.
//!
//! Instructions access memory in two steps: `mem_ref` checks the access and
//! can fault; `mem_read` and `mem_write` then cannot. A read-modify-write
//! instruction checks its destination for writing before it reads it, so
//! nothing is written by an instruction that later faults.

use super::fault::{CpuResult, Fault};
use super::regs::{ATTR_DB, RIGHT_READ, RIGHT_WRITE};
use super::{Cpu, Seg};

/// Kind of memory access, for protection checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// A memory operand whose access has been checked: its bytes can be read
/// and written without faulting. An operand that crosses a page boundary
/// continues at `phys2`, the physical address of the next page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemRef {
    pub lin: u32,
    pub phys: u32,
    pub phys2: u32,
    pub size: u8,
}

/// The fault a segment limit or type violation raises: #SS for the stack
/// segment, #GP for the others.
#[inline(always)]
fn seg_fault(seg: Seg) -> Fault {
    if seg == Seg::SS { Fault::ss(0) } else { Fault::gp(0) }
}

impl Cpu {
    /// Whether the current privilege level is 3, whose accesses the page
    /// tables' user/supervisor bits restrict.
    #[inline(always)]
    fn user(&self) -> bool {
        self.cpl == 3
    }

    /// Linear address of an access of `size` bytes at `seg:off`, checked
    /// against the segment's limit and type.
    #[inline(always)]
    pub fn seg_linear(&self, seg: Seg, off: u32, size: u8, access: Access) -> CpuResult<u32> {
        let cache = &self.seg[seg as usize];
        let last = off.wrapping_add(size as u32 - 1);
        let need = if access == Access::Write { RIGHT_WRITE } else { RIGHT_READ };
        if off < cache.lo || last > cache.hi || last < off || cache.rights & need == 0 {
            return Err(seg_fault(seg));
        }
        Ok(cache.base.wrapping_add(off))
    }

    /// Check an access of `size` bytes at `seg:off`.
    #[inline(always)]
    pub fn mem_ref(&mut self, seg: Seg, off: u32, size: u8, access: Access) -> CpuResult<MemRef> {
        let lin = self.seg_linear(seg, off, size, access)?;
        let user = self.user();
        self.lin_ref(lin, size, access, user)
    }

    /// Check an access of `size` bytes at a linear address: translate it,
    /// and the next page too if it crosses into one.
    #[inline(always)]
    pub fn lin_ref(&mut self, lin: u32, size: u8, access: Access, user: bool) -> CpuResult<MemRef> {
        let write = access == Access::Write;
        let phys = self.lin_to_phys(lin, write, user)?;
        let phys2 = if lin & 0xFFF > 0x1000 - size as u32 {
            self.lin_to_phys((lin | 0xFFF).wrapping_add(1), write, user)?
        } else {
            0
        };
        Ok(MemRef { lin, phys, phys2, size })
    }

    /// Physical address of byte `i` of a checked operand.
    #[inline(always)]
    fn ref_byte(r: MemRef, i: u32) -> usize {
        let in_first = 0x1000 - (r.phys & 0xFFF);
        if i < in_first { (r.phys + i) as usize } else { (r.phys2 + i - in_first) as usize }
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
                value |= (self.bus.read_8(Self::ref_byte(r, i)) as u32) << (8 * i);
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
                self.bus.write_8(Self::ref_byte(r, i), (value >> (8 * i)) as u8);
            }
        }
    }

    /// Read `size` bytes of a system structure (descriptor tables, the
    /// TSS) at a linear address, as a supervisor access.
    pub fn sys_read(&mut self, lin: u32, size: u8) -> CpuResult<u32> {
        let r = self.lin_ref(lin, size, Access::Read, false)?;
        Ok(self.mem_read(r))
    }

    pub fn sys_read_u32(&mut self, lin: u32) -> CpuResult<u32> {
        self.sys_read(lin, 4)
    }

    /// Write `size` bytes of a system structure, see `sys_read`.
    pub fn sys_write(&mut self, lin: u32, size: u8, value: u32) -> CpuResult {
        let r = self.lin_ref(lin, size, Access::Write, false)?;
        self.mem_write(r, value);
        Ok(())
    }

    pub fn sys_write_u8(&mut self, lin: u32, value: u8) -> CpuResult {
        self.sys_write(lin, 1, value as u32)
    }

    /// Check `len` bytes at `seg:off` for an access, for instructions whose
    /// operand is larger than a dword (FPU operands, FSAVE images). Returns
    /// the linear address, which `lin_read_8` and friends then read and
    /// write without faulting.
    pub fn check_span(&mut self, seg: Seg, off: u32, len: u32, access: Access) -> CpuResult<u32> {
        let cache = self.seg[seg as usize];
        let last = off.wrapping_add(len - 1);
        let need = if access == Access::Write { RIGHT_WRITE } else { RIGHT_READ };
        if off < cache.lo || last > cache.hi || last < off || cache.rights & need == 0 {
            return Err(seg_fault(seg));
        }
        let lin = cache.base.wrapping_add(off);
        let user = self.user();
        // Every page the span touches.
        let mut page = lin & !0xFFF;
        let end = lin.wrapping_add(len - 1) & !0xFFF;
        loop {
            self.lin_to_phys(if page < lin { lin } else { page }, access == Access::Write, user)?;
            if page == end {
                break;
            }
            page = page.wrapping_add(0x1000);
        }
        Ok(lin)
    }

    /// Physical address of a byte of a span `check_span` accepted.
    fn span_byte(&mut self, lin: usize, write: bool) -> usize {
        let user = self.user();
        match self.lin_to_phys(lin as u32, write, user) {
            Ok(phys) => phys as usize,
            // The span was checked, so this only happens if the operand
            // overlaps the page tables it changes.
            Err(_) => {
                self.bus.log_string(&format!("[CPU] Unmapped operand byte at {:08X}", lin));
                0xFFFF_FFFF
            }
        }
    }

    /// Read bytes of a checked span, see `check_span`.
    pub fn lin_read_8(&mut self, lin: usize) -> u8 {
        let p = self.span_byte(lin, false);
        self.bus.read_8(p)
    }

    pub fn lin_read_16(&mut self, lin: usize) -> u16 {
        u16::from_le_bytes([self.lin_read_8(lin), self.lin_read_8(lin + 1)])
    }

    pub fn lin_read_32(&mut self, lin: usize) -> u32 {
        self.lin_read_16(lin) as u32 | (self.lin_read_16(lin + 2) as u32) << 16
    }

    pub fn lin_read_64(&mut self, lin: usize) -> u64 {
        self.lin_read_32(lin) as u64 | (self.lin_read_32(lin + 4) as u64) << 32
    }

    /// Write bytes of a checked span, see `check_span`.
    pub fn lin_write_8(&mut self, lin: usize, value: u8) {
        let p = self.span_byte(lin, true);
        self.bus.write_8(p, value);
    }

    pub fn lin_write_16(&mut self, lin: usize, value: u16) {
        self.lin_write_8(lin, value as u8);
        self.lin_write_8(lin + 1, (value >> 8) as u8);
    }

    pub fn lin_write_32(&mut self, lin: usize, value: u32) {
        self.lin_write_16(lin, value as u16);
        self.lin_write_16(lin + 2, (value >> 16) as u16);
    }

    pub fn lin_write_64(&mut self, lin: usize, value: u64) {
        self.lin_write_32(lin, value as u32);
        self.lin_write_32(lin + 4, (value >> 32) as u32);
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

    /// Read a word at a linear address with paging off, as the CPU reads
    /// the real-mode interrupt vector table.
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
