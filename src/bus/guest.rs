//! Memory as the BIOS's services reach it: by the linear address their
//! caller's segment and offset make (`Cpu::real_linear`), as a BIOS's own
//! code would read and write it. With paging on, as under Windows, whose
//! virtual machines have memory of their own at those addresses, that goes
//! through the page tables (`guest_paging`); a page that isn't there ends
//! the service, which runs again once the system has put it there
//! (`exec::service_trap`).

use super::Bus;
use crate::cpu::paging::walk_tables;

impl Bus {
    /// The physical address of the linear address `lin` of a BIOS service,
    /// or None when the page tables have no page for the access there (the
    /// first such page is kept in `guest_fault`).
    fn guest_physical(&mut self, lin: u32, write: bool) -> Option<usize> {
        let Some(paging) = self.guest_paging else {
            return Some((lin & self.a20_mask) as usize);
        };
        if self.guest_fault.is_some() {
            return None;
        }
        match walk_tables(self, paging, lin, write) {
            Ok(walked) => Some(((walked.page | (lin & 0xFFF)) & self.a20_mask) as usize),
            Err(error) => {
                self.guest_fault = Some((lin, error));
                None
            }
        }
    }

    /// Whether a service's access found a page missing: it has to run
    /// again, and must not act on what it read.
    pub fn guest_faulted(&self) -> bool {
        self.guest_fault.is_some()
    }

    pub fn guest_read_8(&mut self, lin: u32) -> u8 {
        self.guest_physical(lin, false).map_or(0, |addr| self.read_8(addr))
    }

    pub fn guest_read_16(&mut self, lin: u32) -> u16 {
        u16::from_le_bytes([self.guest_read_8(lin), self.guest_read_8(lin.wrapping_add(1))])
    }

    pub fn guest_read_32(&mut self, lin: u32) -> u32 {
        self.guest_read_16(lin) as u32 | (self.guest_read_16(lin.wrapping_add(2)) as u32) << 16
    }

    pub fn guest_write_8(&mut self, lin: u32, value: u8) {
        if let Some(addr) = self.guest_physical(lin, true) {
            self.write_8(addr, value);
        }
    }

    pub fn guest_write_16(&mut self, lin: u32, value: u16) {
        let [lo, hi] = value.to_le_bytes();
        self.guest_write_8(lin, lo);
        self.guest_write_8(lin.wrapping_add(1), hi);
    }

    pub fn guest_write_32(&mut self, lin: u32, value: u32) {
        self.guest_write_16(lin, value as u16);
        self.guest_write_16(lin.wrapping_add(2), (value >> 16) as u16);
    }

    /// The parts of `len` bytes at `lin` that lie in one page each: their
    /// offset in the range and length.
    fn page_runs(lin: u32, len: usize) -> impl Iterator<Item = (usize, usize)> {
        let mut done = 0;
        std::iter::from_fn(move || {
            (done < len).then(|| {
                let at = lin.wrapping_add(done as u32);
                let run = (0x1000 - (at & 0xFFF) as usize).min(len - done);
                let part = (done, run);
                done += run;
                part
            })
        })
    }

    /// Fill `data` from the memory at `lin`.
    pub fn guest_read_bytes(&mut self, lin: u32, data: &mut [u8]) {
        for (offset, run) in Self::page_runs(lin, data.len()) {
            let Some(addr) = self.guest_physical(lin.wrapping_add(offset as u32), false) else {
                return;
            };
            for (i, byte) in data[offset..offset + run].iter_mut().enumerate() {
                *byte = self.read_8(addr + i);
            }
        }
    }

    /// Write `data` to the memory at `lin`.
    pub fn guest_write_bytes(&mut self, lin: u32, data: &[u8]) {
        for (offset, run) in Self::page_runs(lin, data.len()) {
            let Some(addr) = self.guest_physical(lin.wrapping_add(offset as u32), true) else {
                return;
            };
            if self.is_plain_ram(addr, run) {
                self.load_bytes(addr, &data[offset..offset + run]);
            } else {
                for (i, &byte) in data[offset..offset + run].iter().enumerate() {
                    self.write_8(addr + i, byte);
                }
            }
        }
    }
}
